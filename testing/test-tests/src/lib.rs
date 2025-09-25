#![cfg_attr(not(test), no_std)]

extern crate alloc;

use testing::description::kernel_test_setup;
use testing_derive::multitest;

kernel_test_setup!();

#[multitest]
mod tests {
    use alloc::boxed::Box;
    use core::any::Any;
    use log::debug;
    use shared::sync::CoreInfo;
    use testing::{
        description::TestExitState,
        kernel_test,
        multiprocessor::{DataBarrier, TestInterruptState},
        t_assert, t_assert_eq, t_assert_ne, tfail, KernelTestError, TestUnwrapExt,
    };

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_tfail() -> Result<(), KernelTestError> {
        tfail!()
    }

    #[kernel_test(allow_heap_leak)]
    fn test_kernel_heap_mem_leak() -> Result<(), KernelTestError> {
        let b = Box::new(5);
        core::mem::forget(b);
        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_tfail_message() -> Result<(), KernelTestError> {
        tfail!("with message: {}", "fail variable");
    }

    #[kernel_test]
    fn test_t_assert_true() -> Result<(), KernelTestError> {
        t_assert!(true);
        t_assert!(true, "This is a message that we should never see!");
        t_assert!(
            true,
            "This is a message that we should never see with args: {}!",
            12 * 5
        );

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_false() -> Result<(), KernelTestError> {
        #[allow(clippy::disallowed_names)]
        let foo = false;
        t_assert!(foo);
        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_false_message() -> Result<(), KernelTestError> {
        t_assert!(false, "Message: {}", 5 * 12);
        Ok(())
    }

    #[kernel_test]
    fn test_t_assert_eq() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 21);
        t_assert_eq!(a, 2 * 21, "some message");
        t_assert_eq!(a, 2 * 21, "some message with args: {}, {}", a, 42);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_eq_fail() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 22);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_eq_fail_message() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 22, "Some message {a}");

        Ok(())
    }

    #[kernel_test]
    fn test_t_assert_ne() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 22);
        t_assert_ne!(a, 2 * 22, "some message");
        t_assert_ne!(a, 2 * 22, "some message with args: {}, {}", a, 42);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_ne_fail() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 21);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_ne_fail_message() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 21, "Some message {a}");

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_t_unwrap_message() -> Result<(), KernelTestError> {
        let o: Option<()> = None;
        o.tunwrap()?;

        Ok(())
    }

    #[kernel_test(i)]
    fn ignored_test() -> Result<(), KernelTestError> {
        tfail!("ignored test should never execute");
    }

    // NOTE: this test needs to be commented out, otherwise it will be focused
    // diabling all other tests
    //    #[kernel_test(i, x)]
    //    fn focused_ignore_test() -> Result<(), KernelTestError> {
    //        Ok(())
    //    }

    #[kernel_test(mp)]
    fn test_multiprocessor_empty() -> Result<(), KernelTestError> {
        Ok(())
    }

    #[kernel_test(mp)]
    fn test_multiprocessor_barrier(
        db: &DataBarrier<Box<dyn Any + Send>>,
    ) -> Result<(), KernelTestError> {
        let data = if TestInterruptState::s_is_bsp() {
            let data = Box::new(2u64);
            db.enter_with_data(data)
        } else {
            db.enter()
        }
        .tunwrap()?;

        let value = data.downcast_ref::<u64>().tunwrap()?;
        t_assert_eq!(2, *value);

        Ok(())
    }

    #[kernel_test(mp, expected_exit: TestExitState::Panic)]
    fn test_multiprocessor_ap_panic() -> Result<(), KernelTestError> {
        if TestInterruptState::max_core_count() == 1 {
            panic!("Test only works in mp environment");
        }

        debug!("in panicing test");
        if TestInterruptState::s_is_bsp() {
            Ok(())
        } else {
            panic!("expected panic on ap");
        }
    }

    #[kernel_test(mp, expected_exit: TestExitState::Error(None))]
    fn test_multiprocessor_ap_failure() -> Result<(), KernelTestError> {
        if TestInterruptState::max_core_count() == 1 {
            tfail!("Test only works in mp environment");
        }

        if TestInterruptState::s_is_bsp() {
            Ok(())
        } else {
            tfail!("expected failure on ap");
        }
    }
}
