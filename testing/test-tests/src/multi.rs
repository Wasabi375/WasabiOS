use testing::{kernel_test, t_assert, KernelTestError};

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
