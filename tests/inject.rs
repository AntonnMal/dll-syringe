use dll_syringe::{error::SyringeError, Syringe};

#[allow(unused)]
mod common;

syringe_test! {
    fn inject_with_valid_path_succeeds(
        process: Process,
        payload_path: &Path,
    ) {
        let mut syringe = Syringe::for_process(&process);
        syringe.inject(payload_path).unwrap();
    }
}

syringe_test! {
    fn inject_with_invalid_path_fails_with_remote_io(
        process: Process,
        _payload_path: &Path,
    ) {
        let mut syringe = Syringe::for_process(&process);
        let result = syringe.inject("invalid path");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SyringeError::RemoteIo(_)));
        let io_err = match err {
            SyringeError::RemoteIo(io_err) => io_err,
            _ => unreachable!(),
        };
        assert_eq!(io_err.raw_os_error(), Some(126));
    }
}

syringe_test! {
    fn inject_with_crashed_process_fails_with_io(
        process: Process,
        payload_path: &Path,
    ) {
        let mut syringe = Syringe::for_process(&process);
        process.clone().kill().unwrap();

        let result = syringe.inject(payload_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SyringeError::ProcessInaccessible));
    }
}
