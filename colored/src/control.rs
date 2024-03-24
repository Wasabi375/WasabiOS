//! A couple of functions to enable and disable coloring.

use core::sync::atomic::{AtomicBool, Ordering};

/// A flag to to if coloring should occur.
pub struct ShouldColorize {
    clicolor: bool,
    clicolor_force: Option<bool>,
    // XXX we can't use Option<Atomic> because we can't use &mut references to ShouldColorize
    has_manual_override: AtomicBool,
    manual_override: AtomicBool,
}

/// Use this to force colored to ignore the environment and always/never colorize
/// See example/control.rs
pub fn set_override(override_colorize: bool) {
    SHOULD_COLORIZE.set_override(override_colorize)
}

/// Remove the manual override and let the environment decide if it's ok to colorize
/// See example/control.rs
pub fn unset_override() {
    SHOULD_COLORIZE.unset_override()
}

lazy_static! {
/// The persistent [`ShouldColorize`].
    pub static ref SHOULD_COLORIZE: ShouldColorize = ShouldColorize::default();
}

impl Default for ShouldColorize {
    fn default() -> ShouldColorize {
        ShouldColorize {
            clicolor: true,
            clicolor_force: None,
            has_manual_override: AtomicBool::new(false),
            manual_override: AtomicBool::new(false),
        }
    }
}

impl ShouldColorize {
    /// Returns if the current coloring is expected.
    pub fn should_colorize(&self) -> bool {
        if self.has_manual_override.load(Ordering::Relaxed) {
            return self.manual_override.load(Ordering::Relaxed);
        }

        if let Some(forced_value) = self.clicolor_force {
            return forced_value;
        }

        self.clicolor
    }

    /// Use this to force colored to ignore the environment and always/never colorize
    pub fn set_override(&self, override_colorize: bool) {
        self.has_manual_override.store(true, Ordering::Relaxed);
        self.manual_override
            .store(override_colorize, Ordering::Relaxed);
    }

    /// Remove the manual override and let the environment decide if it's ok to colorize
    pub fn unset_override(&self) {
        self.has_manual_override.store(false, Ordering::Relaxed);
    }

    /* private */
}

#[cfg(test)]
mod specs {
    use super::*;
    use rspec;
    use rspec::*;

    #[test]
    fn clicolor_behavior() {
        use std::io;

        let stdout = &mut io::stdout();
        let mut formatter = rspec::formatter::Simple::new(stdout);
        let mut runner = describe("ShouldColorize", |ctx| {
            ctx.describe("constructors", |ctx| {
                ctx.it("should have a default constructor", || {
                    ShouldColorize::default();
                });
            });

            ctx.describe("when only changing clicolors", |ctx| {
                ctx.it("clicolor == false means no colors", || {
                    let colorize_control = ShouldColorize {
                        clicolor: false,
                        ..ShouldColorize::default()
                    };
                    false == colorize_control.should_colorize()
                });

                ctx.it("clicolor == true means colors !", || {
                    let colorize_control = ShouldColorize {
                        clicolor: true,
                        ..ShouldColorize::default()
                    };
                    true == colorize_control.should_colorize()
                });

                ctx.it("unset clicolors implies true", || {
                    true == ShouldColorize::default().should_colorize()
                });
            });

            ctx.describe("when using clicolor_force", |ctx| {
                ctx.it(
                    "clicolor_force should force to true no matter clicolor",
                    || {
                        let colorize_control = ShouldColorize {
                            clicolor: false,
                            clicolor_force: Some(true),
                            ..ShouldColorize::default()
                        };

                        true == colorize_control.should_colorize()
                    },
                );

                ctx.it(
                    "clicolor_force should force to false no matter clicolor",
                    || {
                        let colorize_control = ShouldColorize {
                            clicolor: true,
                            clicolor_force: Some(false),
                            ..ShouldColorize::default()
                        };

                        false == colorize_control.should_colorize()
                    },
                );
            });

            ctx.describe("using a manual override", |ctx| {
                ctx.it("shoud colorize if manual_override is true, but clicolor is false and clicolor_force also false", || {
                    let colorize_control = ShouldColorize {
                        clicolor: false,
                        clicolor_force: None,
                        has_manual_override: AtomicBool::new(true),
                        manual_override: AtomicBool::new(true),
                        .. ShouldColorize::default()
                    };

                    true == colorize_control.should_colorize()
                });

                ctx.it("should not colorize if manual_override is false, but clicolor is true or clicolor_force is true", || {
                    let colorize_control = ShouldColorize {
                        clicolor: true,
                        clicolor_force: Some(true),
                        has_manual_override: AtomicBool::new(true),
                        manual_override: AtomicBool::new(false),
                        .. ShouldColorize::default()
                    };

                    false == colorize_control.should_colorize()
                })
            });

            ctx.describe("::set_override", |ctx| {
                ctx.it("should exists", || {
                    let colorize_control = ShouldColorize::default();
                    colorize_control.set_override(true);
                });

                ctx.it("set the manual_override property", || {
                    let colorize_control = ShouldColorize::default();
                    colorize_control.set_override(true);
                    {
                        assert_eq!(
                            true,
                            colorize_control.has_manual_override.load(Ordering::Relaxed)
                        );
                        let val = colorize_control.manual_override.load(Ordering::Relaxed);
                        assert_eq!(true, val);
                    }
                    colorize_control.set_override(false);
                    {
                        assert_eq!(
                            true,
                            colorize_control.has_manual_override.load(Ordering::Relaxed)
                        );
                        let val = colorize_control.manual_override.load(Ordering::Relaxed);
                        assert_eq!(false, val);
                    }
                });
            });

            ctx.describe("::unset_override", |ctx| {
                ctx.it("should exists", || {
                    let colorize_control = ShouldColorize::default();
                    colorize_control.unset_override();
                });

                ctx.it("unset the manual_override property", || {
                    let colorize_control = ShouldColorize::default();
                    colorize_control.set_override(true);
                    colorize_control.unset_override();
                    assert_eq!(
                        false,
                        colorize_control.has_manual_override.load(Ordering::Relaxed)
                    );
                });
            });
        });
        runner.add_event_handler(&mut formatter);
        runner.run().unwrap();
    }
}
