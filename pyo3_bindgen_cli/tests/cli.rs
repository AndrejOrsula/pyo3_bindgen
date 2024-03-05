#[cfg(target_arch = "x86_64")]
mod test_cli {
    use assert_cmd::Command;
    use predicates::prelude::*;

    const BIN_NAME: &str = "pyo3_bindgen";

    #[test]
    fn test_cli_help() {
        // Arrange
        let mut cmd = Command::cargo_bin(BIN_NAME).unwrap();

        // Act
        let assert = cmd.arg("-h").assert();

        // Assert
        assert.success().stdout(
            predicate::str::contains(format!("Usage: {BIN_NAME}"))
                .and(predicate::str::contains("Options:"))
                .and(predicate::str::contains("--module-name <MODULE_NAMES>"))
                .and(predicate::str::contains("--output <OUTPUT>")),
        );
    }

    #[test]
    fn test_cli_default() {
        // Arrange
        let mut cmd = Command::cargo_bin(BIN_NAME).unwrap();

        // Act
        let assert = cmd.assert();

        // Assert
        assert.failure().stderr(
            predicate::str::contains("error: the following required arguments")
                .and(predicate::str::contains("--module-name <MODULE_NAMES>"))
                .and(predicate::str::contains(format!("Usage: {BIN_NAME}"))),
        );
    }

    #[test]
    fn test_cli_bindgen_os() {
        // Arrange
        let mut cmd = Command::cargo_bin(BIN_NAME).unwrap();

        // Act
        let assert = cmd.arg("-m").arg("os").assert();

        // Assert
        assert.success();
    }

    #[test]
    fn test_cli_bindgen_sys() {
        // Arrange
        let mut cmd = Command::cargo_bin(BIN_NAME).unwrap();

        // Act
        let assert = cmd.arg("-m").arg("sys").assert();

        // Assert
        assert.success();
    }
}
