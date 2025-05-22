use proc_macro2::{Ident, Punct, Span};
use syn::{
    ext::IdentExt,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
    Error, Expr, PathSegment, Result, Signature, Token,
};

pub struct TestArgs {
    pub name: Option<Expr>,
    pub expected_exit: Option<Expr>,
    pub focus: bool,
    pub ignore: bool,
    pub multiprocessor: bool,
    pub allow_frame_leak: bool,
    pub allow_page_leak: bool,
    pub allow_heap_leak: bool,
    pub allow_mapping_leak: bool,
}

impl TestArgs {
    pub fn is_panicing(&self) -> bool {
        let Some(Expr::Path(path)) = &self.expected_exit else {
            return false;
        };
        let path = &path.path;
        if path.leading_colon.is_some() {
            return false;
        }
        let mut iter = path.segments.iter();
        match iter.next() {
            Some(PathSegment { ident, .. }) if ident.to_string().as_str() == "TestExitState" => {}
            _ => return false,
        }

        match iter.next() {
            Some(PathSegment { ident, .. }) if ident.to_string().as_str() == "Panic" => {}
            _ => return false,
        }

        iter.next().is_none()
    }
}

pub enum TestFunctionVariance {
    Normal,
    MPBarrier,
}

impl TestFunctionVariance {
    pub fn name(&self) -> &'static str {
        match self {
            TestFunctionVariance::Normal => "Normal",
            TestFunctionVariance::MPBarrier => "MPBarrier",
        }
    }
}

impl TryFrom<&Signature> for TestFunctionVariance {
    type Error = Error;

    fn try_from(signature: &Signature) -> core::result::Result<Self, Self::Error> {
        if let Some(unsafety) = signature.unsafety {
            return Err(Error::new_spanned(
                unsafety,
                "Test function must not be unsafe",
            ));
        }
        if let Some(lt) = signature.generics.lt_token {
            let gt = signature
                .generics
                .gt_token
                .expect("Generics shoudl contain '>' if '<' is present");
            return Err(Error::new(
                lt.span()
                    .join(gt.span())
                    .expect("< and > are in the same file"),
                "Test function must follow one of the variances of KernelTestFunction",
            ));
        }

        match signature.inputs.len() {
            0 => Ok(Self::Normal),
            1 => Ok(Self::MPBarrier),
            _ => Err(Error::new_spanned(
                signature,
                "Test function must follow one of the variances of KernelTestFunction",
            )),
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for TestArgs {
    fn default() -> Self {
        TestArgs {
            name: None,
            expected_exit: None,
            focus: false,
            ignore: false,
            multiprocessor: false,
            allow_frame_leak: false,
            allow_page_leak: false,
            allow_heap_leak: false,
            allow_mapping_leak: false,
        }
    }
}

impl Parse for TestArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut name = None;
        let mut expected_exit = None;
        let mut focus = false;
        let mut ignore = false;
        let mut multiprocessor = false;
        let mut allow_frame_leak = false;
        let mut allow_page_leak = false;
        let mut allow_heap_leak = false;
        let mut allow_mapping_leak = false;

        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(Ident::peek_any) {
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
                    "focus" | "x" | "f" => {
                        if !focus {
                            focus = true;
                        } else {
                            return Err(Error::new_spanned(ident, "focus can only be set once"));
                        }
                    }
                    "ignore" | "i" => {
                        if !ignore {
                            ignore = true;
                        } else {
                            return Err(Error::new_spanned(ident, "ignore can only be set once"));
                        }
                    }
                    "multiprocessor" | "mp" => {
                        if !multiprocessor {
                            multiprocessor = true;
                        } else {
                            return Err(Error::new_spanned(
                                ident,
                                "multiprocessor can only be set once",
                            ));
                        }
                    }
                    "allow_frame_leak" => {
                        if !allow_frame_leak {
                            allow_frame_leak = true;
                        } else {
                            return Err(Error::new_spanned(
                                ident,
                                "\"allow_frame_leak\" can only be set once",
                            ));
                        }
                    }
                    "allow_page_leak" => {
                        if !allow_page_leak {
                            allow_page_leak = true;
                        } else {
                            return Err(Error::new_spanned(
                                ident,
                                "\"allow_page_leak\" can only be set once",
                            ));
                        }
                    }
                    "allow_heap_leak" => {
                        if !allow_heap_leak {
                            allow_heap_leak = true;
                        } else {
                            return Err(Error::new_spanned(
                                ident,
                                "\"allow_heap_leak\" can only be set once",
                            ));
                        }
                    }
                    "allow_mapping_leak" => {
                        if !allow_mapping_leak {
                            allow_mapping_leak = true;
                        } else {
                            return Err(Error::new_spanned(
                                ident,
                                "\"allow_mapping_leak\" can only be set once",
                            ));
                        }
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

        Ok(TestArgs {
            name,
            expected_exit,
            focus,
            ignore,
            multiprocessor,
            allow_frame_leak,
            allow_page_leak,
            allow_heap_leak,
            allow_mapping_leak,
        })
    }
}

pub struct CrateList(pub Punctuated<Ident, Token![,]>);

impl Parse for CrateList {
    fn parse(input: ParseStream) -> Result<Self> {
        let punctuated = input.parse_terminated(Ident::parse, Token![,])?;
        Ok(CrateList(punctuated))
    }
}
