use std::path::PathBuf;
use std::process::Command;

struct DocumentationFixture(PathBuf);

impl DocumentationFixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("pyo3_bindgen_docs_{}_{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let fixture = Self(path);
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::create_dir(fixture.0.join("scripts")).unwrap();
        std::fs::create_dir_all(fixture.0.join("docs/nested")).unwrap();
        std::fs::copy(
            workspace_root().join("scripts/check-docs.sh"),
            fixture.0.join("scripts/check-docs.sh"),
        )
        .unwrap();
        fixture.write("docs/release_checklist.md", "# Release plan\n");
        fixture.write("README.md", "# Overview\n");
        fixture.write("docs/nested/contract.md", "# Contract\n");
        fixture.write(
            "docs/README.md",
            "[Plan](release_checklist.md)\n[Overview](../README.md)\n[Contract](nested/contract.md)\n",
        );
        fixture
    }

    fn write(&self, path: &str, contents: &str) {
        std::fs::write(self.0.join(path), contents).unwrap();
    }

    fn check(&self) -> std::process::Output {
        Command::new("bash")
            .arg(self.0.join("scripts/check-docs.sh"))
            .current_dir(&self.0)
            .env("PYTHON_BIN", "python3")
            .output()
            .unwrap()
    }
}

impl Drop for DocumentationFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("CLI crate is nested below the workspace root")
        .to_path_buf()
}

#[test]
fn active_document_index_and_local_links_are_complete() {
    let root = workspace_root();
    let output = Command::new("bash")
        .arg(root.join("scripts/check-docs.sh"))
        .current_dir(&root)
        .env("PYTHON_BIN", "python3")
        .output()
        .expect("documentation contract script starts");

    assert!(
        output.status.success(),
        "documentation contract failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn documentation_contract_covers_nested_new_files_and_root_links() {
    let fixture = DocumentationFixture::new();
    assert!(fixture.check().status.success());

    fixture.write("docs/nested/unfinished.md", "# Unfinished work\n");
    let orphan = fixture.check();
    assert!(!orphan.status.success());
    assert!(String::from_utf8_lossy(&orphan.stderr).contains("docs/nested/unfinished.md"));
    std::fs::remove_file(fixture.0.join("docs/nested/unfinished.md")).unwrap();

    fixture.write("README.md", "[Missing](deleted-plan.md)\n");
    let broken = fixture.check();
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stderr).contains("missing local Markdown target"));

    fixture.write("README.md", "[Outside](../outside.md)\n");
    let escaped = fixture.check();
    assert!(!escaped.status.success());
    assert!(String::from_utf8_lossy(&escaped.stderr).contains("local link escapes repository"));
}
